git checkout -b part1 62e7e1f

cargo run --release --bin racsma -- calibrate .\assets\acsma\INPUT.bin  -f .\output.bin

git checkout main

git branch -D part1
