package stackless

type Create struct {
	On          string
	File        string
	Name        string
	Sources     []string
	Dirty       bool
	Lease       string
	ConfirmPaid bool
}

type Resume struct {
	Name    string
	File    string
	Sources []string
	Dirty   bool
	Lease   string
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

type UpOutcome struct {
	Instance     string
	Substrate    string
	Origins      map[string]string
	Integrations map[string]map[string]string
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
	Stack     string
	Substrate string
	Services  []string
	Graph     map[string]any
}

type StatusReport map[string]any

type ListOutcome struct {
	Instances           []map[string]any
	PersistenceWarning  string
	Raw                 map[string]any
}
