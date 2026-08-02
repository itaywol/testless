package com.example.calc;

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Nested;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.ValueSource;

/// Lives in package `com.example.calc` but under `src/test/java`, so it
/// references `Calc` with no import at all — the parallel-source-root case
/// that a directory-keyed package scope would miss.
class CalcTest {
    private final Calc calc = new Calc();

    @Test
    void addsNegatives() {
        assertEquals(-3, calc.add(-1, -2));
    }

    @Test
    void multiplies() {
        assertEquals(6, calc.mul(2, 3));
    }

    @ParameterizedTest
    @ValueSource(ints = {1, 2, 3})
    void addsZeroIdentity(int n) {
        assertEquals(n, calc.add(n, 0));
    }

    @Nested
    class WhenDoubling {
        @Test
        void doublesTheSum() {
            assertEquals(6, calc.addTwice(1, 2));
        }
    }
}
