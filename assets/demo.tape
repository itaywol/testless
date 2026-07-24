# testless demo. Regenerate:
#   1. cargo build --release -p testless
#   2. demo repo at /tmp/demo: src/{math,format,greet}.ts + matching *.test.ts
#      (fmt calls add; greet unrelated), .gitignore with ".testless/", git committed
#   3. JetBrains Mono visible to fontconfig (XDG_DATA_HOME with fonts/)
#   4. cd /tmp/demo && PATH=.../target/release:$PATH vhs this-file

Output "/home/itay/Work/testless/assets/demo.gif"

Set Shell "bash"
Set FontFamily "JetBrains Mono"
Set FontSize 26
Set Width 1400
Set Height 800
Set Theme "Catppuccin Mocha"
Set Padding 30
Set Margin 0
Set BorderRadius 0
Set WindowBar Colorful
Set TypingSpeed 35ms
Set Framerate 24

Hide
Type `export PS1='\[\e[1;35m\]❯\[\e[0m\] ' && clear`
Enter
Sleep 300ms
Type "testless index"
Show

Sleep 700ms
Enter
Sleep 2s

Type "sed -i 's/a + b;/a + b + 1;/' src/math.ts   # change one function"
Enter
Sleep 1200ms

Type "testless select --from HEAD"
Enter
Sleep 2.5s

Type "testless select --from HEAD --format args"
Enter
Sleep 4s
