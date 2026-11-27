# Records the tmux session demo/record.sh starts; demo/say.sh types into it.
# Run through demo/record.sh, not on its own.
Output raw.mp4

Set Shell "bash"
Set FontSize 17
Set Width 1200
Set Height 720
Set Padding 18
Set Theme "Catppuccin Mocha"
Set Framerate 25
Set CursorBlink false
Set WaitTimeout 40m

Hide
Type "tmux -u -L pathfinder-demo attach -t demo"
Enter
Sleep 1.5s
Show

# demo/say.sh --end quits the tmux server, which ends the recording.
Wait+Screen@40m /server exited|lost server|exited\]/
Sleep 300ms
