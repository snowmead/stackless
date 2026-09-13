package stackless

type Create struct {
	AllowHostExecution bool
	On                 string
	File               string
	Name               string
	Sources            []string
	Dirty              bool
	Lease              string
	ConfirmPaid        bool
}

type Resume struct {
	AllowHostExecution bool
	Name               string
	File               string
	Sources            []string
	Dirty              bool
	Lease              string
}

type UpRequest struct {
	Create *Create
	Resume *Resume
}

func UpCreate(c Create) UpRequest {
	return UpRequest{Create: &c}
}

func UpResume(r Resume) UpRequest {
	return UpRequest{Resume: &r}
}

type SecretRef struct {
	Kind        string `json:"kind"`
	InstanceID  string `json:"instance_id"`
	Integration string `json:"integration"`
	Output      string `json:"output"`
}

type EndpointBinding struct {
	Workload string `json:"workload"`
	URL      string `json:"url"`
	Source   string `json:"source"`
}

type Placements struct {
	Workloads map[string]string `json:"workloads"`
	Resources map[string]string `json:"resources"`
}

type UpOutcome struct {
	InstanceID   string
	Instance     string
	Substrate    string
	Origins      map[string]string
	Endpoints    map[string]EndpointBinding
	Placements   Placements
	Integrations map[string]map[string]SecretRef
	Executed     []string
	Skipped      []string
	DurationMs   uint64
	Steps        []any
	Spend        any
}

type DownOutcome struct {
	Instance string
	Status   string
	Spend    any
}

type VerifyOutcome struct {
	Instance           string
	Tier               string
	DurationMs         uint64
	ExitStatus         int
	LogPath            string
	LeaseRemainingSecs *uint64
}

type LogsOutcome struct {
	Instance  string
	Substrate string
	Available *bool
	Services  []map[string]any
}

type CheckOutcome struct {
	Placements *Placements
	Stack      string
	Substrate  string
	Services   []string
	Graph      map[string]any
}

type StatusReport map[string]any

type ListOutcome struct {
	Instances          []map[string]any
	PersistenceWarning string
	Raw                map[string]any
}

// Operation survives the calling CLI or SDK process.
type Operation struct {
	ID              string `json:"id"`
	Instance        string `json:"instance"`
	Verb            string `json:"verb"`
	Status          string `json:"status"`
	Result          any    `json:"result"`
	Error           any    `json:"error"`
	CancelRequested bool   `json:"cancel_requested"`
	CreatedAt       int64  `json:"created_at"`
	UpdatedAt       int64  `json:"updated_at"`
}

type OperationEvent struct {
	Sequence int64 `json:"sequence"`
	Event    any   `json:"event"`
}

type OperationPage struct {
	Operation Operation        `json:"operation"`
	Events    []OperationEvent `json:"events"`
}

// EndpointURLs supplies the URL map consumed by generated endpoint bindings.
func (o *UpOutcome) EndpointURLs() map[string]string {
	urls := make(map[string]string, len(o.Endpoints))
	for name, endpoint := range o.Endpoints {
		urls[name] = endpoint.URL
	}
	return urls
}
