package stackless

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
)

type ExecRunner interface {
	Run(bin string, args []string, cwd string) (stdout, stderr []byte, exitCode int, err error)
}

type defaultRunner struct{}

func (defaultRunner) Run(bin string, args []string, cwd string) ([]byte, []byte, int, error) {
	cmd := exec.Command(bin, args...)
	if cwd != "" {
		cmd.Dir = cwd
	}
	// Buffer both streams (do not StdoutPipe then drain serially): `up --json`
	// streams NDJSON progress on stderr while the success envelope waits on
	// stdout, so a filled stderr pipe would deadlock the child.
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	waitErr := cmd.Run()
	exitCode := 0
	if waitErr != nil {
		if ee, ok := waitErr.(*exec.ExitError); ok {
			exitCode = ee.ExitCode()
		} else {
			return stdout.Bytes(), stderr.Bytes(), -1, waitErr
		}
	}
	return stdout.Bytes(), stderr.Bytes(), exitCode, nil
}

type Client struct {
	bin        string
	cwd        string
	runner     ExecRunner
	controller string
}

// WithController returns a client that sends lifecycle calls through an SSH host.
func (c *Client) WithController(target string) *Client {
	copy := *c
	copy.controller = target
	return &copy
}

func (c *Client) SetRunner(r ExecRunner) {
	if r != nil {
		c.runner = r
	}
}

func System() *Client {
	return New("", "")
}

func New(bin, cwd string) *Client {
	if bin == "" {
		bin = resolveBin()
	}
	if cwd != "" {
		cwd = filepath.Clean(cwd)
	}
	return &Client{bin: bin, cwd: cwd, runner: defaultRunner{}}
}

func resolveBin() string {
	if v := os.Getenv("STACKLESS_BIN"); v != "" {
		return v
	}
	return "stackless"
}

func (c *Client) invoke(args []string) (map[string]any, error) {
	full := append([]string{"--json"}, args...)
	if c.controller != "" {
		full = append([]string{"--json", "--controller", c.controller}, args...)
	}
	stdout, stderr, exitCode, err := c.runner.Run(c.bin, full, c.cwd)
	if err != nil {
		return nil, err
	}
	text := trimJSON(stdout)
	if text != "" {
		var data map[string]any
		if json.Unmarshal([]byte(text), &data) == nil {
			okVal, exists := data["ok"]
			if exists {
				if b, isBool := okVal.(bool); isBool {
					if b {
						return data, nil
					}
					return nil, errorFromEnvelope(data)
				}
			}
		}
	}
	msg := string(stderr)
	if msg == "" {
		msg = fmt.Sprintf("exit status %d", exitCode)
	}
	return nil, &Error{Code: "cli_failed", Message: msg, ExitStatus: exitCode, Stderr: string(stderr)}
}

func trimJSON(b []byte) string {
	start := 0
	end := len(b)
	for start < end && (b[start] == ' ' || b[start] == '\n' || b[start] == '\r' || b[start] == '\t') {
		start++
	}
	for end > start && (b[end-1] == ' ' || b[end-1] == '\n' || b[end-1] == '\r' || b[end-1] == '\t') {
		end--
	}
	return string(b[start:end])
}

func upArgs(req UpRequest) ([]string, error) {
	var args []string
	args = append(args, "up")
	switch {
	case req.Create != nil:
		r := req.Create
		args = append(args, "--on", r.On)
		if r.Name != "" {
			args = append(args, "--name", r.Name)
		}
		if r.File != "" {
			args = append(args, "--file", r.File)
		}
		for _, s := range r.Sources {
			args = append(args, "--source", s)
		}
		if r.AllowHostExecution {
			args = append(args, "--allow-host-execution")
		}
		if r.Dirty {
			args = append(args, "--dirty")
		}
		if r.Lease != "" {
			args = append(args, "--lease", r.Lease)
		}
		if r.ConfirmPaid {
			args = append(args, "--confirm-paid")
		}
	case req.Resume != nil:
		r := req.Resume
		args = append(args, "--name", r.Name)
		if r.File != "" {
			args = append(args, "--file", r.File)
		}
		for _, s := range r.Sources {
			args = append(args, "--source", s)
		}
		if r.AllowHostExecution {
			args = append(args, "--allow-host-execution")
		}
		if r.Dirty {
			args = append(args, "--dirty")
		}
		if r.Lease != "" {
			args = append(args, "--lease", r.Lease)
		}
	default:
		return nil, &Error{Code: "bad_argument", Message: "up requires Create or Resume"}
	}
	return args, nil
}

func (c *Client) Up(req UpRequest) (*UpOutcome, error) {
	args, err := upArgs(req)
	if err != nil {
		return nil, err
	}
	data, err := c.invoke(args)
	if err != nil {
		return nil, err
	}
	instanceID := asString(data["instance_id"])
	integrations, err := secretRefs(data["integrations"], instanceID)
	if err != nil {
		return nil, err
	}
	endpoints, err := endpointBindings(data["endpoints"])
	if err != nil {
		return nil, err
	}
	placements, err := placementsMap(data["placements"])
	if err != nil {
		return nil, err
	}
	return &UpOutcome{
		InstanceID:   instanceID,
		Instance:     asString(data["instance"]),
		Substrate:    asString(data["substrate"]),
		Origins:      originsMap(data["origins"]),
		Endpoints:    endpoints,
		Placements:   placements,
		Integrations: integrations,
		Executed:     asStringSlice(data["executed"]),
		Skipped:      asStringSlice(data["skipped"]),
		DurationMs:   asUint64(data["duration_ms"]),
		Steps:        asAnySlice(data["steps"]),
		Spend:        data["spend"],
	}, nil
}

func (c *Client) Down(name string) (*DownOutcome, error) {
	data, err := c.invoke([]string{"down", name})
	if err != nil {
		return nil, err
	}
	status := asString(data["outcome"])
	if status == "" {
		status = asString(data["status"])
	}
	return &DownOutcome{
		Instance: asString(data["instance"]),
		Status:   status,
		Spend:    data["spend"],
	}, nil
}

func (c *Client) Verify(name string, tier string) (*VerifyOutcome, error) {
	args := []string{"verify", name}
	if tier != "" {
		args = append(args, "--tier", tier)
	}
	data, err := c.invoke(args)
	if err != nil {
		return nil, err
	}
	var lease *uint64
	if v, ok := data["lease_remaining_secs"]; ok && v != nil {
		n := asUint64(v)
		lease = &n
	}
	return &VerifyOutcome{
		Instance:           asString(data["instance"]),
		Tier:               asString(data["tier"]),
		DurationMs:         asUint64(data["duration_ms"]),
		ExitStatus:         asInt(data["exit_status"]),
		LogPath:            asString(data["log_path"]),
		LeaseRemainingSecs: lease,
	}, nil
}

func (c *Client) Status(name string) (StatusReport, error) {
	return c.invoke([]string{"status", name})
}

func (c *Client) List() (*ListOutcome, error) {
	data, err := c.invoke([]string{"list"})
	if err != nil {
		return nil, err
	}
	var instances []map[string]any
	if raw, ok := data["instances"].([]any); ok {
		for _, item := range raw {
			if m, ok := item.(map[string]any); ok {
				instances = append(instances, m)
			}
		}
	}
	return &ListOutcome{
		Instances:          instances,
		PersistenceWarning: asString(data["persistence_warning"]),
		Raw:                data,
	}, nil
}

func (c *Client) Logs(name, service string, tail int) (*LogsOutcome, error) {
	args := []string{"logs", name}
	if service != "" {
		args = append(args, service)
	}
	if tail > 0 {
		args = append(args, "--tail", fmt.Sprintf("%d", tail))
	}
	data, err := c.invoke(args)
	if err != nil {
		return nil, err
	}
	sub := asString(data["substrate"])
	var avail *bool
	if sub != "" {
		f := false
		avail = &f
	} else if v, ok := data["available"].(bool); ok {
		avail = &v
	} else {
		t := true
		avail = &t
	}
	return &LogsOutcome{
		Instance:  asString(data["instance"]),
		Substrate: sub,
		Available: avail,
		Services:  asMapSlice(data["services"]),
	}, nil
}

func (c *Client) Check(file, on string) (*CheckOutcome, error) {
	args := []string{"check", file}
	if on != "" {
		args = append(args, "--on", on)
	}
	data, err := c.invoke(args)
	if err != nil {
		return nil, err
	}
	graph, _ := data["graph"].(map[string]any)
	if graph == nil {
		graph = map[string]any{}
	}
	var placements *Placements
	if data["placements"] != nil {
		value, err := placementsMap(data["placements"])
		if err != nil {
			return nil, err
		}
		placements = &value
	}
	return &CheckOutcome{
		Placements: placements,
		Stack:      asString(data["stack"]),
		Substrate:  asString(data["substrate"]),
		Services:   asStringSlice(data["services"]),
		Graph:      graph,
	}, nil
}

func originsMap(raw any) map[string]string {
	out := map[string]string{}
	list, ok := raw.([]any)
	if !ok {
		return out
	}
	for _, item := range list {
		m, ok := item.(map[string]any)
		if !ok {
			continue
		}
		svc := asString(m["service"])
		origin := asString(m["origin"])
		if svc != "" && origin != "" {
			out[svc] = origin
		}
	}
	return out
}

func secretRefs(raw any, instanceID string) (map[string]map[string]SecretRef, error) {
	out := map[string]map[string]SecretRef{}
	if instanceID == "" {
		return nil, fmt.Errorf("up response lacks an immutable instance ID")
	}
	if raw == nil {
		return out, nil
	}
	top, ok := raw.(map[string]any)
	if !ok {
		return nil, fmt.Errorf("invalid integration reference map")
	}
	for integration, val := range top {
		outputs, ok := val.(map[string]any)
		if !ok {
			return nil, fmt.Errorf("invalid integration reference map")
		}
		out[integration] = map[string]SecretRef{}
		for output, val := range outputs {
			ref, ok := val.(map[string]any)
			if !ok || len(ref) != 4 || ref["kind"] != "secret_ref" || ref["instance_id"] != instanceID || ref["integration"] != integration || ref["output"] != output {
				return nil, fmt.Errorf("invalid or foreign integration secret reference")
			}
			out[integration][output] = SecretRef{Kind: "secret_ref", InstanceID: instanceID, Integration: integration, Output: output}
		}
	}
	return out, nil
}

func asString(v any) string {
	s, _ := v.(string)
	return s
}

func asStringSlice(v any) []string {
	raw, ok := v.([]any)
	if !ok {
		return nil
	}
	out := make([]string, 0, len(raw))
	for _, item := range raw {
		if s, ok := item.(string); ok {
			out = append(out, s)
		}
	}
	return out
}

func asAnySlice(v any) []any {
	raw, ok := v.([]any)
	if !ok {
		return nil
	}
	return raw
}

func asMapSlice(v any) []map[string]any {
	raw, ok := v.([]any)
	if !ok {
		return nil
	}
	out := make([]map[string]any, 0, len(raw))
	for _, item := range raw {
		if m, ok := item.(map[string]any); ok {
			out = append(out, m)
		}
	}
	return out
}

func asUint64(v any) uint64 {
	switch n := v.(type) {
	case float64:
		return uint64(n)
	case int:
		return uint64(n)
	case int64:
		return uint64(n)
	case uint64:
		return n
	default:
		return 0
	}
}

func asInt(v any) int {
	switch n := v.(type) {
	case float64:
		return int(n)
	case int:
		return n
	case int64:
		return int(n)
	default:
		return 0
	}
}

func decodeOperationValue[T any](value any) (*T, error) {
	encoded, err := json.Marshal(value)
	if err != nil {
		return nil, err
	}
	var result T
	if err = json.Unmarshal(encoded, &result); err != nil {
		return nil, err
	}
	return &result, nil
}

func (c *Client) SubmitUp(req UpRequest) (*Operation, error) {
	args, err := upArgs(req)
	if err != nil {
		return nil, err
	}
	data, err := c.invoke(append(args, "--no-wait"))
	if err != nil {
		return nil, err
	}
	return decodeOperationValue[Operation](data["operation"])
}

func (c *Client) SubmitDown(name string) (*Operation, error) {
	data, err := c.invoke([]string{"down", name, "--no-wait"})
	if err != nil {
		return nil, err
	}
	return decodeOperationValue[Operation](data["operation"])
}

func (c *Client) Operation(id string, after int64) (*OperationPage, error) {
	data, err := c.invoke([]string{"operation", "get", id, "--after", fmt.Sprint(after)})
	if err != nil {
		return nil, err
	}
	return decodeOperationValue[OperationPage](data["result"])
}

func (c *Client) CancelOperation(id string) (*Operation, error) {
	data, err := c.invoke([]string{"operation", "cancel", id})
	if err != nil {
		return nil, err
	}
	return decodeOperationValue[Operation](data["operation"])
}

func (c *Client) WaitOperation(id string) (any, error) {
	data, err := c.invoke([]string{"operation", "wait", id})
	if err != nil {
		return nil, err
	}
	return data["result"], nil
}

func (c *Client) Operations(instance string) ([]Operation, error) {
	args := []string{"operation", "list"}
	if instance != "" {
		args = append(args, "--instance", instance)
	}
	data, err := c.invoke(args)
	if err != nil {
		return nil, err
	}
	result, err := decodeOperationValue[[]Operation](data["result"])
	if err != nil {
		return nil, err
	}
	return *result, nil
}

func endpointBindings(raw any) (map[string]EndpointBinding, error) {
	bindings := map[string]EndpointBinding{}
	if raw == nil {
		return bindings, nil
	}
	values, ok := raw.(map[string]any)
	if !ok {
		return nil, &Error{Message: "invalid endpoint binding map"}
	}
	for name, rawValue := range values {
		value, ok := rawValue.(map[string]any)
		if !ok {
			return nil, &Error{Message: "invalid endpoint binding"}
		}
		workload, wok := value["workload"].(string)
		url, uok := value["url"].(string)
		source, sok := value["source"].(string)
		if !wok || workload == "" || !uok || url == "" || !sok || (source != "provider" && source != "declared") {
			return nil, &Error{Message: "invalid endpoint binding"}
		}
		bindings[name] = EndpointBinding{Workload: workload, URL: url, Source: source}
	}
	return bindings, nil
}

func placementsMap(raw any) (Placements, error) {
	result := Placements{Workloads: map[string]string{}, Resources: map[string]string{}}
	if raw == nil {
		return result, nil
	}
	values, ok := raw.(map[string]any)
	if !ok {
		return result, &Error{Message: "invalid placements"}
	}
	for kind, target := range map[string]map[string]string{"workloads": result.Workloads, "resources": result.Resources} {
		entries, ok := values[kind].(map[string]any)
		if !ok {
			return result, &Error{Message: "invalid placement map"}
		}
		for name, value := range entries {
			on, ok := value.(string)
			if !ok || on == "" || name == "" {
				return result, &Error{Message: "invalid hosting placement"}
			}
			target[name] = on
		}
	}
	return result, nil
}
