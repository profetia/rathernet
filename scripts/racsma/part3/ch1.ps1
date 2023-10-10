git checkout -b part3 522620e

cargo build --release

# Set both sound card to output 40%, input 15%

# Set windows sound output device to devices other than the sound card

# Set ASIO driver buffer size to 64

# Play the jamming wav
./target/release/raudio write ./assets/acsma/Jamming.wav

# Start node 2, because it takes longer to load
./target/release/racsma duplex .\INPUT2to1.bin -n 50000 -a 1 -o 0 -f .\output1to2.bin

# Start node 1, the one that has better hardware
./target/release/racsma duplex .\INPUT1to2.bin -n 40000 -a 0 -o 1 -f .\output2to1.bin

git checkout main

git branch -D part3
