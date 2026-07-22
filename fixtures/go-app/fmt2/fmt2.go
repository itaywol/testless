package fmt2

import (
	"fmt"

	"example.com/go-app/calc"
)

func Fmt(a, b int) string { return fmt.Sprintf("%d", calc.Add(a, b)) }
