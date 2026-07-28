package stackless

import "fmt"

type Error struct {
	Code         string
	Message      string
	Remediation  string
	Raw          map[string]any
	ExitStatus   int
	Stderr       string
}

func (e *Error) Error() string {
	if e.Message != "" {
		return e.Message
	}
	return fmt.Sprintf("stackless: %s", e.Code)
}

func errorFromEnvelope(data map[string]any) *Error {
	errObj, _ := data["error"].(map[string]any)
	if errObj == nil {
		return &Error{Code: "unknown", Message: "stackless command failed"}
	}
	code, _ := errObj["code"].(string)
	msg, _ := errObj["message"].(string)
	rem, _ := errObj["remediation"].(string)
	return &Error{
		Code:        code,
		Message:     msg,
		Remediation: rem,
		Raw:         errObj,
	}
}
