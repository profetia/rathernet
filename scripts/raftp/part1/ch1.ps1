git checkout -b part1 39a2d5f

cargo build --release

# Start node 2 in administrator mode as the NAT server
# You should not have wireshark running in the background
./target/release/rateway nat -c ./assets/ateway/node2.toml

# Start node 1 as the client
./target/release/rateway install -c ./assets/ateway/node1.toml

# Wait for the installation to complete
# Set up the route table

# Start the client
./target/release/raftp "<user>@<host>[:port]"

# Interact with the client
# Run the following commands in the client
# `pwd` - print the current working directory
# `ls` - list the files in the current working directory
# `cd <dir>` - change the current working directory to <dir>
# `get <file>` - download <file> from the server

git checkout main

git branch -D part1
