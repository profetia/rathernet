git checkout -b part1 fcdbc73

cargo build --release

# Start node 3 as the receiver
./target/release/rateway udp receive -a "<ipv4:port>"

# Start node 2 as the sender
./target/release/rateway udp send -a "<ipv4:port>" "<ipv4:port>"

git checkout main

git branch -D part1
