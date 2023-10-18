git checkout -b part6 242ebd9

cargo build --release

# Start node 2 in administrator mode as the NAT server
./target/release/rateway nat -c ./assets/ateway/node2.toml

# Start node 1 as the sender
./target/release/rateway install -c ./assets/ateway/node1.toml

# Wait for the installation to complete
# Set up the route table

# Start node 3 as the receiver
./target/release/rateway tcp receive -a "<ipv4:port>"

# Send file from node 1 to node 3
./target/release/rateway tcp send "<ipv4:port>" -s ./assets/ateway/INPUT.txt

git checkout main

git branch -D part6
