package fmt2

import "testing"

func TestFmt(t *testing.T) {
	if Fmt(1, 2) != "3" { t.Fail() }
}
