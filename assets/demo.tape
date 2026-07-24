# testless demo: regenerate with:
#   1. cargo build --release -p testless
#   2. create a demo repo at /tmp/demo:
#        mkdir -p /tmp/demo/src && cd /tmp/demo && git init -q
#        cat > src/math.ts <<'TS'
#        export function add(a: number, b: number) { return a + b; }
#        export function sub(a: number, b: number) { return a - b; }
#        TS
#        cat > src/math.test.ts <<'TS'
#        import { expect, test } from "vitest";
#        import { add, sub } from "./math";
#        test("add", () => expect(add(1, 2)).toBe(3));
#        test("sub", () => expect(sub(3, 1)).toBe(2));
#        TS
#        cat > src/format.ts <<'TS'
#        import { add } from "./math";
#        export function fmt(a: number, b: number) { return `${add(a, b)}`; }
#        TS
#        cat > src/format.test.ts <<'TS'
#        import { expect, test } from "vitest";
#        import { fmt } from "./format";
#        test("fmt", () => expect(fmt(1, 2)).toBe("3"));
#        TS
#        cat > src/greet.ts <<'TS'
#        export function greet(name: string) { return `hello, ${name}`; }
#        TS
#        cat > src/greet.test.ts <<'TS'
#        import { expect, test } from "vitest";
#        import { greet } from "./greet";
#        test("greet", () => expect(greet("a")).toBe("hello, a"));
#        TS
#        echo ".testless/" > .gitignore
#        git add -A && git commit -q -m "demo repo"
#   3. export PATH="/home/itay/Work/testless/target/release:$PATH"
#   4. cd /tmp/demo
#   5. nix-shell -p vhs ttyd chromium ffmpeg jetbrains-mono --run \
#        'fc-cache -f >/dev/null && vhs /home/itay/Work/testless/assets/demo.tape'
#      (JetBrains Mono must be available via fontconfig for FontFamily to
#      resolve; the nix-shell above provides both vhs's runtime deps and
#      the font, and fc-cache registers it before vhs launches its terminal.)

Output "/home/itay/Work/testless/assets/demo.gif"

Set Shell "bash"
Set FontFamily "JetBrains Mono"
Set FontSize 26
Set Width 1400
Set Height 800
Set Theme "Catppuccin Mocha"
Set Padding 30
Set Margin 16
Set MarginFill "#7287fd"
Set BorderRadius 10
Set WindowBar Colorful
Set TypingSpeed 45ms
Set Framerate 24

Hide
Type `export PS1='\[\e[1;35m\]❯\[\e[0m\] ' && clear`
Enter
Sleep 800ms
Show

Sleep 600ms
Type "testless index"
Enter
Sleep 2s

Type "sed -i 's/a + b;/a + b + 1;/' src/math.ts   # change one function"
Enter
Sleep 1200ms

Type "testless select --from HEAD"
Enter
Sleep 3s

Type "testless select --from HEAD --format args"
Enter
Sleep 4s
