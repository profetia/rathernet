git checkout -b part1 0288e7b

# Start node 2 as the receiver
cargo run --release --bin rateway -- calibrate -t read -a <ipv4:port>

# Start node 1 as the sender
cargo run --release --bin rateway -- calibrate -t write -a <ipv4:port> -p <ipv4:port>

git checkout main

git branch -D part1
