git checkout -b part7 ffb83f6

cargo build --release

# Set both sound card to output 40%, input 15%

# Set windows sound output device to devices other than the sound card

# Set ASIO driver buffer size to 64

# Start node 1
./target/release/racsma duplex .\INPUT1to2.bin -n 50000 -a 0 -o 1 -f .\output2to1.bin

# Start node 2
./target/release/racsma duplex .\INPUT2to1.bin -n 50000 -a 1 -o 0 -f .\output1to2.bin

# Start node 3
./target/release/racsma duplex .\INPUT3to4.bin -n 50000 -a 2 -o 3 -f .\output4to3.bin

# Start node 4
./target/release/racsma duplex .\INPUT4to3.bin -n 50000 -a 3 -o 2 -f .\output3to4.bin

git checkout main

git branch -D part7
