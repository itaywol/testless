import { it, expect } from "vitest";
import { fmt } from "./index";
it("formats", () => { expect(fmt(1, 2)).toBe("3"); });
