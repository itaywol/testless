package calc

import "testing"

func TestAdd(t *testing.T) {
	t.Run("negatives", func(t *testing.T) {
		if Add(-1, -2) != -3 { t.Fail() }
	})
	t.Run("zero", func(t *testing.T) {
		if Add(0, 5) != 5 { t.Fail() }
	})
}

func TestCalc(t *testing.T) {
	cases := []struct{ name string }{{"a"}, {"b"}}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {})
	}
}

func BenchmarkAdd(b *testing.B) { for range b.N { Add(1, 2) } }
