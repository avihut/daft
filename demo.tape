# VHS Demo Tape for daft
# Run: vhs demo.tape

Output demo.gif
Set WindowBar Colorful
Set FontSize 16
Set Width 800
Set Height 400
Set Theme "Pro"
Set Padding 20
Set TypingSpeed 40ms

Env PATH "/Users/avihu/Projects/daft/target/release:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"

# Hidden cleanup
Hide
Type "git push origin -d feature/demo"
Enter
Sleep 2s
Type "cd /tmp && rm -rf daft"
Enter
Sleep 2s
Type "clear"
Enter
Sleep 500ms
Show

# Scene 1: Clone
Type "git worktree-clone git@github.com:avihut/daft.git"
Enter
Sleep 8s

# Scene 2: Go into the cloned repo and create feature branch
Type "cd daft/master && git worktree-checkout-branch feature/demo"
Enter
Sleep 5s

# Scene 3: Show the structure
Enter
Type "ls -l /tmp/daft/"
Enter 2
Sleep 3s
Type "git worktree list"
Enter
Sleep 5s
