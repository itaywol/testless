package calc

func Add(a, b int) int { return a + b }

type Calc struct{ Total int }

func (c *Calc) Push(n int) { c.Total = Add(c.Total, n) }

func init() { _ = Add(0, 0) }
