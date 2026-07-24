# testless demo — regenerate with:
#   cd /tmp/testless-demo && export PATH="$(pwd)/../../home/itay/Work/testless/target/release:$PATH"  # adjust as needed
#   nix-shell -p vhs ttyd chromium ffmpeg --run 'vhs assets/demo.tape'
#
# Prerequisites (outside this recording):
#   1. cargo build --release -p testless
#   2. a demo TS repo at /tmp/testless-demo:
#        - src/math.ts   (add, sub)
#        - src/format.ts (fmt, calls add)
#        - src/greet.ts  (unrelated to add/fmt)
#        - matching *.test.ts for each, committed with git
#   3. cwd = /tmp/testless-demo, PATH includes .../target/release, before invoking vhs.
#
# Terminal: 100 cols x 28 rows at FontSize 20 => Width 2150, Height 814.

Output "/home/itay/Work/testless/assets/demo.gif"

Set Shell "bash"
Set FontSize 20
Set Width 2150
Set Height 814
Set Theme "Catppuccin Mocha"
Set TypingSpeed 60ms
Set Padding 20
Set Framerate 30

Sleep 500ms

Type "testless index"
Enter
Sleep 2s

Type "sed -i 's/return a + b;/return a + b + 1;/' src/math.ts"
Enter
Sleep 1s

Type "testless select --from HEAD"
Enter
Sleep 2.5s

Type "testless select --from HEAD --format args"
Enter
Sleep 3s
