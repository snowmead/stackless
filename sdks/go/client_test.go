package stackless

import (
	"encoding/json"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"testing"
)

func TestHostExecutionRequiresExplicitCallerFlag(t *testing.T) {
	for _, allowed := range []bool{false, true} {
		for _, request := range []UpRequest{
			UpCreate(Create{On: "local", AllowHostExecution: allowed}),
			UpResume(Resume{Name: "demo", AllowHostExecution: allowed}),
		} {
			args, err := upArgs(request)
			if err != nil {
				t.Fatal(err)
			}
			if slices.Contains(args, "--allow-host-execution") != allowed {
				t.Fatalf("host grant %v produced args %v", allowed, args)
			}
		}
	}
}

type fakeRunner struct {
	byKey map[string]map[string]any
}

func (f fakeRunner) Run(bin string, args []string, cwd string) ([]byte, []byte, int, error) {
	key := keyArgs(args[1:]...)
	payload := f.byKey[key]
	out, _ := json.Marshal(payload)
	code := 0
	if ok, _ := payload["ok"].(bool); !ok {
		code = 1
	}
	return out, nil, code, nil
}

func keyArgs(args ...string) string {
	b, _ := json.Marshal(args)
	return string(b)
}

func TestControllerIsForwardedToSubmissionsAndOperationReads(t *testing.T) {
	base := New("/fake/stackless", "")
	base.SetRunner(fakeRunner{byKey: map[string]map[string]any{
		keyArgs("--controller", "ssh://deploy@builder", "down", "demo", "--no-wait"):              {"ok": true, "operation": map[string]any{"id": "op"}},
		keyArgs("--controller", "ssh://deploy@builder", "operation", "get", "op", "--after", "0"): {"ok": true, "result": map[string]any{"operation": map[string]any{"id": "op"}, "events": []any{}}},
	}})
	c := base.WithController("ssh://deploy@builder")
	if base.controller != "" {
		t.Fatal("WithController mutated the original client")
	}
	if _, err := c.SubmitDown("demo"); err != nil {
		t.Fatal(err)
	}
	if _, err := c.Operation("op", 0); err != nil {
		t.Fatal(err)
	}
}

func TestUpOriginsAndIntegrations(t *testing.T) {
	payload := map[string]any{
		"schema_version": 1,
		"ok":             true,
		"instance":       "demo",
		"instance_id":    "owner-1",
		"substrate":      "local",
		"executed":       []any{"start:web"},
		"skipped":        []any{},
		"duration_ms":    12,
		"steps":          []any{},
		"origins": []any{
			map[string]any{"service": "web", "origin": "http://demo.localhost:4444/"},
		},
		"placements": map[string]any{"workloads": map[string]any{"web": "local", "api": "fly"}, "resources": map[string]any{"clerk": "local"}},
		"endpoints":  map[string]any{"public": map[string]any{"workload": "web", "url": "https://public.example.test", "source": "declared"}},
		"integrations": map[string]any{
			"clerk": map[string]any{"secret_key": map[string]any{"kind": "secret_ref", "instance_id": "owner-1", "integration": "clerk", "output": "secret_key"}},
		},
	}
	c := New("/fake/stackless", "")
	c.SetRunner(fakeRunner{byKey: map[string]map[string]any{
		keyArgs("up", "--on", "local", "--name", "demo"): payload,
	}})

	out, err := c.Up(UpCreate(Create{On: "local", Name: "demo"}))
	if err != nil {
		t.Fatal(err)
	}
	if out.Placements.Workloads["api"] != "fly" || out.Placements.Resources["clerk"] != "local" {
		t.Fatal(out.Placements)
	}
	if out.Endpoints["public"].Source != "declared" || out.EndpointURLs()["public"] != "https://public.example.test" {
		t.Fatalf("endpoint missing: %v", out.Endpoints)
	}
	if out.Origins["web"] != "http://demo.localhost:4444/" {
		t.Fatalf("origin: %q", out.Origins["web"])
	}
	if out.Integrations["clerk"]["secret_key"].InstanceID != "owner-1" {
		t.Fatalf("integration key missing")
	}
}

func TestDownErrorCode(t *testing.T) {
	c := New("/fake/stackless", "")
	c.SetRunner(fakeRunner{byKey: map[string]map[string]any{
		keyArgs("down", "missing"): {
			"ok": false,
			"error": map[string]any{
				"code":    "instance_not_found",
				"message": "no such instance",
			},
		},
	}})
	_, err := c.Down("missing")
	var se *Error
	if !errors.As(err, &se) || se.Code != "instance_not_found" {
		t.Fatalf("expected instance_not_found, got %v", err)
	}
}

func TestListCheck(t *testing.T) {
	c := New("/fake/stackless", "")
	c.SetRunner(fakeRunner{byKey: map[string]map[string]any{
		keyArgs("list"): {
			"schema_version":      1,
			"ok":                  true,
			"instances":           []any{},
			"persistence_warning": "leases ephemeral",
		},
		keyArgs("check", "stackless.toml"): {
			"schema_version": 1,
			"ok":             true,
			"stack":          "demo",
			"services":       []any{"web"},
			"graph":          map[string]any{"nodes": []any{}},
		},
	}})
	listed, err := c.List()
	if err != nil || len(listed.Instances) != 0 {
		t.Fatalf("list: %v", err)
	}
	check, err := c.Check("stackless.toml", "")
	if err != nil || check.Stack != "demo" {
		t.Fatalf("check: %v", err)
	}
}

func TestDefaultRunnerDrainsStderrBeforeStdoutCloses(t *testing.T) {
	if _, err := exec.LookPath("python3"); err != nil {
		t.Skip("python3 required")
	}
	// Flood stderr past typical pipe capacity, then emit stdout JSON — the old
	// serial StdoutPipe/StderrPipe drain hung here.
	stdout, stderr, code, err := defaultRunner{}.Run("python3", []string{"-c", `
import sys
sys.stderr.write("x" * 256000)
sys.stderr.flush()
sys.stdout.write('{"ok": true, "schema_version": 1}')
sys.stdout.flush()
`}, "")
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	if code != 0 {
		t.Fatalf("exit %d stderr=%q", code, stderr)
	}
	var data map[string]any
	if json.Unmarshal(stdout, &data) != nil || data["ok"] != true {
		t.Fatalf("stdout: %s", stdout)
	}
	if len(stderr) < 256000 {
		t.Fatalf("stderr truncated: %d", len(stderr))
	}
}

func TestRejectsPlaintextAndForeignReferences(t *testing.T) {
	for _, ref := range []any{"sk_test_CANARY", map[string]any{"kind": "secret_ref", "instance_id": "foreign", "integration": "clerk", "output": "secret_key"}} {
		_, err := secretRefs(map[string]any{"clerk": map[string]any{"secret_key": ref}}, "owner-1")
		if err == nil {
			t.Fatal("unsafe reference accepted")
		}
	}
}

func TestInvalidEndpointBindings(t *testing.T) {
	for _, raw := range []any{[]any{}, "url", map[string]any{"public": map[string]any{"workload": "web", "url": "https://example.test", "source": "invented"}}, map[string]any{"public": map[string]any{"url": "https://example.test", "source": "provider"}}} {
		if _, err := endpointBindings(raw); err == nil {
			t.Fatalf("accepted invalid endpoint: %v", raw)
		}
	}
}

func TestGeneratedEndpointBindingNames(t *testing.T) {
	generated, err := os.ReadFile("../../crates/stackless-idl/testdata/endpoints.go")
	if err != nil {
		t.Fatal(err)
	}
	root := t.TempDir()
	files := map[string][]byte{
		"go.mod":       []byte("module endpointfixture\n\ngo 1.22\n"),
		"endpoints.go": generated,
		"endpoints_test.go": []byte(`package stacklessbind
import "testing"
func TestBindings(t *testing.T) {
    if _, err := BindEndpoints(map[string]string{}); err == nil { t.Fatal("missing endpoint accepted") }
    bindings, err := BindEndpoints(map[string]string{"native-api":"http://native.example.test", "public-api":"https://public.example.test/v1"})
    if err != nil { t.Fatal(err) }
    if bindings.NativeApi != "http://native.example.test" || bindings.PublicApi != "https://public.example.test/v1" { t.Fatalf("wrong bindings: %+v", bindings) }
}
`),
	}
	for name, contents := range files {
		if err := os.WriteFile(filepath.Join(root, name), contents, 0600); err != nil {
			t.Fatal(err)
		}
	}
	cmd := exec.Command("go", "test", "./...")
	cmd.Dir = root
	if output, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("generated endpoint bindings failed: %v\n%s", err, output)
	}
}

func TestInvalidPlacements(t *testing.T) {
	for _, value := range []any{[]any{}, "local", map[string]any{}, map[string]any{"workloads": map[string]any{"api": false}, "resources": map[string]any{}}, map[string]any{"workloads": map[string]any{}, "resources": map[string]any{"db": ""}}} {
		payload := map[string]any{"ok": true, "instance": "demo", "instance_id": "owner-1", "substrate": "local", "origins": []any{}, "placements": value}
		c := New("/fake/stackless", "")
		c.SetRunner(fakeRunner{byKey: map[string]map[string]any{keyArgs("up", "--on", "local"): payload}})
		if _, err := c.Up(UpCreate(Create{On: "local"})); err == nil {
			t.Fatalf("accepted invalid placements: %#v", value)
		}
	}
}
