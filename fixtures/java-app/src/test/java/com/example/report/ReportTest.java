package com.example.report;

import static org.junit.jupiter.api.Assertions.assertEquals;

import com.example.calc.Calc;
import org.junit.jupiter.api.Test;

class ReportTest {
    @Test
    void summarizes() {
        assertEquals("sum=3", new Report(new Calc()).summarize(1, 2));
    }
}
