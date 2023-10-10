git checkout -b part4 fe867da

cargo build --release

# Set both sound card to output 40%, input 15%

# Set windows sound output device to devices other than the sound card

# Set ASIO driver buffer size to 64

# Start node 2 as the server
./target/release/racsma serve -a 1
# Start node 1 to start perf node 1
./target/release/racsma perf -a 0 -p 1

# Start node 1 as the server
./target/release/racsma serve -a 0
# Start node 2 to start perf node 2
./target/release/racsma perf -a 1 -p 0

git checkout main

git branch -D part4
