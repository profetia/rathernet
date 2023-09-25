git checkout -b part3 9a43ff4

cargo build --release

./target/release/rather duplex -c ./assets/ather/INPUT.txt -f ./output.txt

git checkout main

git branch -D part3
