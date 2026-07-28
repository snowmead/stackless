package stackless

import (
	"encoding/json"
	"errors"
	"testing"
)

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

func TestUpOriginsAndIntegrations(t *testing.T) {
	payload := map[string]any{
		"schema_version": 1,
		"ok":             true,
		"instance":       "demo",
		"substrate":      "local",
		"executed":       []any{"start:web"},
		"skipped":        []any{},
		"duration_ms":    12,
		"steps":          []any{},
		"origins": []any{
			map[string]any{"service": "web", "origin": "http://demo.localhost:4444/"},
		},
		"integrations": map[string]any{
			"clerk": map[string]any{"secret_key": "sk_test", "publishable_key": "pk_test"},
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
	if out.Origins["web"] != "http://demo.localhost:4444/" {
		t.Fatalf("origin: %q", out.Origins["web"])
	}
	if out.Integrations["clerk"]["secret_key"] != "sk_test" {
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
