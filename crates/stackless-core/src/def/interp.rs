//! The interpolation namespace (ARCHITECTURE.md §1).
//!
//! Env values reference a namespace evaluated per instance per
//! substrate. `$PORT` is deliberately *not* an interpolation reference —
//! it is injected by the local substrate into `run` commands only, so
//! the tokenizer here only understands `${...}` forms.

use std::collections::BTreeMap;

use super::error::DefError;

/// A parsed `${...}` reference.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reference {
    /// `${instance.name}`
    InstanceName,
    /// `${services.X.origin}`
    ServiceOrigin(String),
    /// `${datastores.X.url}`
    DatastoreUrl(String),
    /// `${secrets.KEY}`
    Secret(String),
}

/// Extract every `${...}` reference from a value.
///
/// `location` names where the value lives (e.g. `services.api.env.DATABASE_URL`)
/// so errors point at the right line of the definition.
pub fn references(value: &str, location: &str) -> Result<Vec<Reference>, DefError> {
    let mut refs = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(DefError::ReferenceSyntax {
                location: location.to_owned(),
                reference: rest[start..].to_owned(),
                detail: "unterminated ${...}".into(),
            });
        };
        refs.push(parse_reference(&after[..end], location)?);
        rest = &after[end + 1..];
    }
    Ok(refs)
}

fn parse_reference(inner: &str, location: &str) -> Result<Reference, DefError> {
    let parts: Vec<&str> = inner.split('.').collect();
    let reference = match parts.as_slice() {
        ["instance", "name"] => Reference::InstanceName,
        ["services", name, "origin"] => Reference::ServiceOrigin((*name).to_owned()),
        ["datastores", name, "url"] => Reference::DatastoreUrl((*name).to_owned()),
        ["secrets", key] => Reference::Secret((*key).to_owned()),
        _ => {
            return Err(DefError::ReferenceSyntax {
                location: location.to_owned(),
                reference: format!("${{{inner}}}"),
                detail: "not a recognized namespace form".into(),
            });
        }
    };
    Ok(reference)
}

/// The values a substrate supplies for one instance. Built per instance
/// per substrate; resolution itself is substrate-blind.
#[derive(Debug, Default)]
pub struct Namespace {
    pub instance_name: String,
    pub service_origins: BTreeMap<String, String>,
    pub datastore_urls: BTreeMap<String, String>,
    pub secrets: BTreeMap<String, String>,
}

impl Namespace {
    fn lookup(&self, reference: &Reference, location: &str) -> Result<String, DefError> {
        match reference {
            Reference::InstanceName => Ok(self.instance_name.clone()),
            Reference::ServiceOrigin(name) => {
                self.service_origins.get(name).cloned().ok_or_else(|| {
                    DefError::UndeclaredReference {
                        location: location.to_owned(),
                        kind: "service",
                        name: name.clone(),
                    }
                })
            }
            Reference::DatastoreUrl(name) => {
                self.datastore_urls.get(name).cloned().ok_or_else(|| {
                    DefError::UndeclaredReference {
                        location: location.to_owned(),
                        kind: "datastore",
                        name: name.clone(),
                    }
                })
            }
            Reference::Secret(key) => {
                self.secrets
                    .get(key)
                    .cloned()
                    .ok_or_else(|| DefError::UndeclaredReference {
                        location: location.to_owned(),
                        kind: "secret",
                        name: key.clone(),
                    })
            }
        }
    }
}

/// Substitute every `${...}` in `value` from the namespace.
pub fn resolve(value: &str, namespace: &Namespace, location: &str) -> Result<String, DefError> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(DefError::ReferenceSyntax {
                location: location.to_owned(),
                reference: rest[start..].to_owned(),
                detail: "unterminated ${...}".into(),
            });
        };
        let reference = parse_reference(&after[..end], location)?;
        out.push_str(&namespace.lookup(&reference, location)?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_all_namespace_forms() {
        let refs = references(
            "${instance.name} ${services.web.origin} ${datastores.db.url} ${secrets.KEY}",
            "test",
        )
        .unwrap();
        assert_eq!(
            refs,
            vec![
                Reference::InstanceName,
                Reference::ServiceOrigin("web".into()),
                Reference::DatastoreUrl("db".into()),
                Reference::Secret("KEY".into()),
            ]
        );
    }

    #[test]
    fn port_is_not_a_reference() {
        assert!(references("vite --port $PORT", "test").unwrap().is_empty());
    }

    #[test]
    fn unterminated_reference_fails() {
        assert!(references("${services.web.origin", "test").is_err());
    }

    #[test]
    fn unknown_form_fails() {
        assert!(references("${services.web.port}", "test").is_err());
    }

    #[test]
    fn resolves_mixed_text() {
        let mut namespace = Namespace {
            instance_name: "demo".into(),
            ..Namespace::default()
        };
        namespace
            .service_origins
            .insert("web".into(), "http://web.demo.localhost:4444".into());
        let resolved = resolve(
            "origin=${services.web.origin};i=${instance.name}",
            &namespace,
            "t",
        )
        .unwrap();
        assert_eq!(resolved, "origin=http://web.demo.localhost:4444;i=demo");
    }
}
