export function add(a: number, b: number): number { return a + b; }
export const mul = (a: number, b: number): number => a * b;
export class Calc {
  total = 0;
  push(n: number) { this.total = add(this.total, n); }
}
console.log("side effect at import");
