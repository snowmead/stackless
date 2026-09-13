//! The shared engine crosses provider boundaries without reinterpreting receipts.
#![allow(clippy::unwrap_used)]
use stackless_core::{
    capabilities::Capabilities,
    def::{Namespace, StackDef, interp::resolve},
    engine::{Engine, StepKind, UpRequest},
    routing::RoutedSubstrate,
    state::{Checkpoint, Ownership, ResourceIntent, Store},
    substrate::{
        InstanceContext, NamespacePurpose, Observation, ServiceLog, StepContext, StepResource,
        Substrate, SubstrateFault,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct Evidence {
    live: BTreeSet<String>,
    starts: BTreeMap<String, usize>,
    env: BTreeMap<String, BTreeMap<String, String>>,
    destroyed: Vec<String>,
    lose_start: Option<String>,
    unavailable_logs: BTreeSet<String>,
}
struct Provider {
    name: &'static str,
    early: bool,
    evidence: Arc<Mutex<Evidence>>,
    barrier: Option<Arc<tokio::sync::Barrier>>,
}
#[async_trait::async_trait]
impl Substrate for Provider {
    fn name(&self) -> &str {
        self.name
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            early_origins: self.early,
            ..Capabilities::local()
        }
    }
    fn validate_definition(&self, _: &StackDef) -> Result<(), SubstrateFault> {
        Ok(())
    }
    fn supports_source_override(&self) -> bool {
        self.early
    }
    fn default_lease(&self) -> Duration {
        Duration::from_secs(if self.early { 86400 } else { 28800 })
    }
    fn service_origin(
        &self,
        _: &StackDef,
        instance: &InstanceContext<'_>,
        service: &str,
    ) -> String {
        format!("https://{service}.{}.{}.test", instance.name, self.name)
    }
    fn build_namespace(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        prior: &[Checkpoint],
        _: &BTreeMap<String, String>,
        _: NamespacePurpose,
    ) -> Namespace {
        let mut ns = Namespace::default();
        for name in def.services.keys() {
            // Deliberately derive foreign names too, as existing native adapters do.
            if self.early || prior.iter().any(|cp| cp.step_id == format!("start:{name}")) {
                ns.service_origins
                    .insert(name.clone(), self.service_origin(def, instance, name));
            }
        }
        instance.bind_namespace(&mut ns, def);
        ns
    }
    async fn execute(&self, ctx: StepContext<'_>) -> Result<StepResource, SubstrateFault> {
        let id = format!("{}:{}:{}", self.name, ctx.instance.id, ctx.step.id);
        if ctx.step.kind == StepKind::Start {
            if let Some(barrier) = &self.barrier {
                barrier.wait().await;
            }
            let ns = self.build_namespace(
                ctx.def,
                ctx.instance,
                ctx.prior,
                &BTreeMap::new(),
                NamespacePurpose::ServiceEnv,
            );
            let env = ctx.def.services[&ctx.step.node]
                .env
                .iter()
                .map(|(k, v)| (k.clone(), resolve(v, &ns, k).unwrap()))
                .collect();
            let mut evidence = self.evidence.lock().unwrap();
            evidence.env.insert(ctx.step.node.clone(), env);
            *evidence.starts.entry(ctx.step.node.clone()).or_default() += 1;
        }
        let key = format!("{}:{}:{}", self.name, ctx.operation_id, ctx.step.id);
        ctx.store
            .resource_intent(ResourceIntent {
                owner_id: ctx.instance.id,
                key: &key,
                step_id: &ctx.step.id,
                provider: self.name,
                ownership: Ownership::Owned,
                resource_kind: self.name,
                resource_id: &id,
                payload: "{}",
                dependencies: ctx.parent_resources,
            })
            .unwrap();
        self.evidence.lock().unwrap().live.insert(id.clone());
        if self.evidence.lock().unwrap().lose_start.as_deref() == Some(&ctx.step.id) {
            // No final handle write. Teardown must route this durable create intent.
            return Err(SubstrateFault {
                code: "test.lost_ack".into(),
                message: "lost response".into(),
                remediation: "retry".into(),
                context: Box::default(),
            });
        }
        ctx.store
            .resource_created(ctx.instance.id, &key, &id, "{}")
            .unwrap();
        Ok(StepResource {
            resource_kind: self.name.into(),
            resource_id: id,
            payload: "{}".into(),
        })
    }
    async fn observe(
        &self,
        _: &InstanceContext<'_>,
        cp: &Checkpoint,
    ) -> Result<Observation, SubstrateFault> {
        assert_eq!(
            cp.resource_kind, self.name,
            "a different provider decoded this receipt"
        );
        Ok(
            if self.evidence.lock().unwrap().live.contains(&cp.resource_id) {
                Observation::Present
            } else {
                Observation::Gone
            },
        )
    }
    async fn destroy(
        &self,
        _: &InstanceContext<'_>,
        cp: &Checkpoint,
    ) -> Result<(), SubstrateFault> {
        assert_eq!(
            cp.resource_kind, self.name,
            "a different provider destroyed this receipt"
        );
        let mut evidence = self.evidence.lock().unwrap();
        evidence.live.remove(&cp.resource_id);
        evidence.destroyed.push(cp.step_id.clone());
        Ok(())
    }
    async fn fetch_logs(
        &self,
        _: &Store,
        _: &StackDef,
        _: &InstanceContext<'_>,
        services: &[String],
        _: usize,
    ) -> Result<Option<Vec<ServiceLog>>, SubstrateFault> {
        if self
            .evidence
            .lock()
            .unwrap()
            .unavailable_logs
            .contains(self.name)
        {
            return Ok(None);
        }
        Ok(Some(
            services
                .iter()
                .map(|service| ServiceLog {
                    service: service.clone(),
                    source: self.name,
                    log_path: None,
                    lines: vec![self.name.into()],
                })
                .collect(),
        ))
    }
}
fn router(
    store: &Store,
    definition: &StackDef,
    evidence: &Arc<Mutex<Evidence>>,
    barrier: Option<Arc<tokio::sync::Barrier>>,
) -> RoutedSubstrate {
    let recorded = store
        .instance("demo")
        .unwrap()
        .map(|record| store.placements(&record.instance_id).unwrap())
        .unwrap_or_default();
    let providers = [("near", true), ("far", false)]
        .into_iter()
        .map(|(name, early)| {
            (
                name.into(),
                Box::new(Provider {
                    name,
                    early,
                    evidence: evidence.clone(),
                    barrier: barrier.clone(),
                }) as Box<dyn Substrate>,
            )
        })
        .collect();
    RoutedSubstrate::new(
        Some(store.clone()),
        "near".into(),
        definition.clone(),
        recorded,
        providers,
    )
    .unwrap()
}
async fn up(
    store: &Store,
    router: &RoutedSubstrate,
    text: &str,
    def: &StackDef,
) -> Result<stackless_core::engine::UpOutcome, stackless_core::engine::EngineError> {
    Engine {
        store,
        substrate: router,
    }
    .up(UpRequest {
        instance: "demo",
        definition_text: text,
        def,
        source_overrides: BTreeMap::new(),
        dirty: false,
        definition_dir: String::new(),
        lease: None,
        progress: None,
    })
    .await
}
const MIXED: &str = r#"
[stack]
name = "mixed"
[workloads.api]
on = "far"
run = "server"
health = { path = "/" }
env = { WEB = "${services.web.origin}" }
[workloads.web]
run = "server"
health = { path = "/" }
env = { API = "${endpoints.api.url}" }
depends_on = { api = "ready" }
[endpoints.api]
workload = "api"
"#;

#[tokio::test]
async fn mixed_outputs_resume_and_retired_receipts_use_their_actual_provider() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::open(&path).unwrap();
    let evidence = Arc::new(Mutex::new(Evidence::default()));
    let def = StackDef::parse(MIXED).unwrap();
    let provider = router(&store, &def, &evidence, None);
    assert_eq!(provider.default_lease().as_secs(), 28800);
    up(&store, &provider, MIXED, &def).await.unwrap();
    {
        let data = evidence.lock().unwrap();
        assert_eq!(data.env["api"]["WEB"], "https://web.demo.near.test");
        assert_eq!(data.env["web"]["API"], "https://api.demo.far.test");
    }
    up(&store, &provider, MIXED, &def).await.unwrap();
    assert_eq!(
        evidence.lock().unwrap().starts,
        BTreeMap::from([("api".into(), 1), ("web".into(), 1)])
    );
    let record = store.instance("demo").unwrap().unwrap();
    let cps = store.checkpoints("demo").unwrap();
    let instance = InstanceContext::from_record(&record, &cps);
    let logs = provider
        .fetch_logs(&store, &def, &instance, &["api".into(), "web".into()], 10)
        .await
        .unwrap()
        .unwrap();
    assert!(
        logs.iter()
            .any(|log| log.service == "api" && log.source == "far")
    );
    assert!(
        logs.iter()
            .any(|log| log.service == "web" && log.source == "near")
    );
    evidence
        .lock()
        .unwrap()
        .unavailable_logs
        .insert("far".into());
    let logs = provider
        .fetch_logs(&store, &def, &instance, &["api".into(), "web".into()], 10)
        .await
        .unwrap()
        .unwrap();
    assert!(
        logs.iter()
            .any(|log| log.service == "api" && log.source == "unavailable" && log.lines.is_empty())
    );
    assert!(
        logs.iter()
            .any(|log| log.service == "web" && log.source == "near")
    );
    evidence.lock().unwrap().unavailable_logs.clear();
    let changed_text = MIXED.replace("on = \"far\"", "on = \"near\"");
    let changed = StackDef::parse(&changed_text).unwrap();
    let changed_provider = router(&store, &changed, &evidence, None);
    assert!(
        up(&store, &changed_provider, &changed_text, &changed)
            .await
            .is_err()
    );
    assert_eq!(store.instance("demo").unwrap().unwrap().definition, MIXED);
    // Reopen and remove the cloud workload. Its placement survives the new definition.
    let empty_text = "[stack]\nname = \"mixed\"\n";
    let empty = StackDef::parse(empty_text).unwrap();
    let reopened = Store::open(&path).unwrap();
    let provider = router(&reopened, &empty, &evidence, None);
    up(&reopened, &provider, empty_text, &empty).await.unwrap();
    assert!(evidence.lock().unwrap().live.is_empty());
    assert_eq!(
        reopened.placements(&record.instance_id).unwrap()["service:api"],
        "far"
    );
    // Once teardown proved absence, this name can be placed on another adapter.
    let provider = router(&reopened, &changed, &evidence, None);
    up(&reopened, &provider, &changed_text, &changed)
        .await
        .unwrap();
    assert_eq!(
        reopened.placements(&record.instance_id).unwrap()["service:api"],
        "near"
    );
    Engine {
        store: &reopened,
        substrate: &provider,
    }
    .down("demo")
    .await
    .unwrap();
    assert!(evidence.lock().unwrap().live.is_empty());
}

#[tokio::test]
async fn independent_providers_start_concurrently() {
    let text = MIXED
        .replace("env = { WEB = \"${services.web.origin}\" }", "")
        .replace("env = { API = \"${endpoints.api.url}\" }", "")
        .replace("depends_on = { api = \"ready\" }", "");
    let def = StackDef::parse(&text).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("state.db")).unwrap();
    let evidence = Arc::new(Mutex::new(Evidence::default()));
    let provider = router(
        &store,
        &def,
        &evidence,
        Some(Arc::new(tokio::sync::Barrier::new(2))),
    );
    tokio::time::timeout(Duration::from_secs(3), up(&store, &provider, &text, &def))
        .await
        .unwrap()
        .unwrap();
    Engine {
        store: &store,
        substrate: &provider,
    }
    .down("demo")
    .await
    .unwrap();
}

#[tokio::test]
async fn lost_cloud_response_is_recovered_from_inventory_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::open(&path).unwrap();
    let evidence = Arc::new(Mutex::new(Evidence {
        lose_start: Some("start:api".into()),
        ..Evidence::default()
    }));
    let def = StackDef::parse(MIXED).unwrap();
    let provider = router(&store, &def, &evidence, None);
    assert!(up(&store, &provider, MIXED, &def).await.is_err());
    assert!(store.checkpoint("demo", "start:api").unwrap().is_none());
    assert!(!evidence.lock().unwrap().live.is_empty());
    let reopened = Store::open(&path).unwrap();
    let provider = router(&reopened, &def, &evidence, None);
    Engine {
        store: &reopened,
        substrate: &provider,
    }
    .down("demo")
    .await
    .unwrap();
    assert!(evidence.lock().unwrap().live.is_empty());
}
