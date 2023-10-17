git checkout -b part2 a8f45e5

cargo build --release

# Start node 2 in administrator mode as the NAT server
./target/release/rateway nat -c ./assets/ateway/node2.toml

# Start node 1 as the sender
./target/release/rateway install -c ./assets/ateway/node1.toml

# Wait for the installation to complete
# Set up the route table

# Start node 1 as the receiver
./target/release/rateway udp receive -a "<ipv4:port>"

# Send file from node 3 to node 1
# Note that the receiver's address is the NAT server's address
# And the receiver's port is the port that is mapped to in ch1
./target/release/rateway udp send -a "<ipv4:port>" "<ipv4:port>" -s ./assets/ateway/INPUT.txt

git checkout main

git branch -D part2
