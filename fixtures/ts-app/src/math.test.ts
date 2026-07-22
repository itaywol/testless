import { describe, it, expect, test } from "vitest";
import { add, mul, Calc } from "./math";

describe("add", () => {
  it("handles negatives", () => { expect(add(-1, -2)).toBe(-3); });
  it("handles zero", () => { expect(add(0, 5)).toBe(5); });
});

test("mul works", () => { expect(mul(2, 3)).toBe(6); });

describe("Calc", () => {
  it(`computed ${"name"}`, () => { expect(new Calc().total).toBe(0); });
});
