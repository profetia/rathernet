git checkout -b part3 a3353f1

cargo build --release

# Start node 2 in administrator mode as the NAT server
./target/release/rateway nat -c ./assets/ateway/node2.toml

# Start node 1 as the sender
./target/release/rateway install -c ./assets/ateway/node1.toml

# Wait for the installation to complete
# Set up the route table

# Ping node 3
ping "<ipv4>" -S "<ipv4>" -t

git checkout main

git branch -D part3
