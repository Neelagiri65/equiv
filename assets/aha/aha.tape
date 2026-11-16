# Records assets/equiv-aha.gif with vhs (https://github.com/charmbracelet/vhs).
# Needs: jellyfish==1.3.0 installed in the python3 on PATH, the equiv v0.32.1
# binary on PATH, and EQUIV_METRICS=off. Run from the repo root:
#   vhs assets/aha/aha.tape
Output assets/equiv-aha.gif
Set Shell "bash"
Set FontSize 16
Set Width 1200
Set Height 640
Set Padding 20
Set TypingSpeed 40ms
Env PS1 "$ "
Hide
Type "cd assets/aha && clear"
Enter
Show
Type "# jellyfish 1.3.0 ships metaphone twice: in Rust and in Python"
Enter
Type "cat rust.py python.py"
Enter
Sleep 2s
Type "equiv review rust.py python.py metaphone str"
Enter
Sleep 4s
Type "# same check again: same receipt id, byte for byte"
Enter
Type "equiv review rust.py python.py metaphone str | grep receipt-id"
Enter
Sleep 5s
