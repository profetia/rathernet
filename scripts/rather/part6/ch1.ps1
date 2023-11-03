git checkout -b part6 88d11b3

cargo build --release

./target/release/rather duplex -c ./assets/ather/INPUT.txt -f ./output.txt

git checkout main

git branch -D part6
