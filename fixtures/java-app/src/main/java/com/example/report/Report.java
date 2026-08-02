package com.example.report;

import com.example.calc.Calc;

/// Cross-package dependency: reaches `Calc` only through a real import, and
/// holds it as a *field type* too, which is the shape dependency-injected
/// Java code takes.
public class Report {
    private final Calc calc;

    public Report(Calc calc) {
        this.calc = calc;
    }

    public String summarize(int a, int b) {
        return "sum=" + calc.add(a, b);
    }
}
