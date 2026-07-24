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
