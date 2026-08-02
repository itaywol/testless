package com.example.calc;

import java.util.ArrayList;
import java.util.List;

public class Calc {
    private final List<Integer> history = new ArrayList<>();

    public int add(int a, int b) {
        int sum = a + b;
        history.add(sum);
        return sum;
    }

    public int mul(int a, int b) {
        return a * b;
    }

    /// Same-package reference with no import statement: only the package
    /// scope extension makes this edge resolvable.
    public int addTwice(int a, int b) {
        return Doubler.twice(add(a, b));
    }

    public int size() {
        return history.size();
    }
}
