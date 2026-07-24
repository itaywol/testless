# testless

Cut your test time to the minimum. Run 5 tests instead of 500 by reading your code, not your coverage.

testless statically analyzes your repo down to the function level, diffs changes structurally, and prints exactly the tests your change could break as ready-to-run vitest / go test / cargo test invocations. Selection is always a superset of the truly impacted tests.

![demo](https://raw.githubusercontent.com/itaywol/testless/main/assets/demo.gif)

```bash
npm i -D testless-cli    # installs the `testless` binary for your platform

testless index
testless select --from origin/main
testless select --from origin/main --format args
```

Website: https://testless.itaywol.tools
Source and docs: https://github.com/itaywol/testless

MIT
