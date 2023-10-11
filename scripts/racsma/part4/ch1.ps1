git checkout -b part4 b3a3011

cargo build --release

# Set both sound card to output 40%, input 15%

# Set windows sound output device to devices other than the sound card

# Set ASIO driver buffer size to 64

# Start node 2 first since it's slower
./target/release/racsma perf -a 1 -p 0
# Start node 1
./target/release/racsma perf -a 0 -p 1

git checkout main

git branch -D part4
