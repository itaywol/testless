# testless demo. Regenerate: see assets/demo.tape history; demo repo at /tmp/demo
# with the one-line change ALREADY applied to src/math.ts before recording.

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
Type "git --no-pager diff    # I changed one function"
Show

Sleep 700ms
Enter
Sleep 2200ms

Type "testless select --from HEAD"
Enter
Sleep 2500ms

Type "testless select --from HEAD --format args"
Enter
Sleep 4s
