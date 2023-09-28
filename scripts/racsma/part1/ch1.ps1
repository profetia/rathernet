git checkout -b part1 c222bb2

cargo run --release --bin racsma -- calibrate .\assets\acsma\INPUT.bin  -f .\output.bin

git checkout main

git branch -D part1
