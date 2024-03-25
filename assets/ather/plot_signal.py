import json
import sys
import matplotlib.pyplot as plt
import numpy as np

# sampling rate
sr = 48000
# sampling interval
ts = 1.0/sr

def draw_amplitude(t, x):
    plt.figure(figsize = (8, 6))
    plt.plot(t, x, 'r')
    plt.ylabel('Amplitude')

    plt.show()

def DFT(x):
    """
    Function to calculate the 
    discrete Fourier Transform 
    of a 1D real-valued signal x
    """

    N = len(x)
    n = np.arange(N)
    k = n.reshape((N, 1))
    e = np.exp(-2j * np.pi * k * n / N)
    
    X = np.dot(e, x)
    
    return X

def draw_spectrum(t, x):
    X = DFT(x)

    # calculate the frequency
    N = len(X)
    n = np.arange(N)
    T = N/sr
    freq = n/T 

    plt.figure(figsize = (8, 6))
    plt.stem(freq, abs(X), 'b', \
            markerfmt=" ", basefmt="-b")
    plt.xlabel('Freq (Hz)')
    plt.ylabel('DFT Amplitude |X(freq)|')
    plt.show()

plt.style.use('seaborn-poster')

if __name__ == "__main__":
    filename = sys.argv[2]

    with open(filename) as f:
        x = json.loads(f.read())

    t = np.arange(0, len(x) * ts - 1e-8, ts)

    if sys.argv[1] == "-a":
        draw_amplitude(t, x)
    elif sys.argv[1] == "-s":
        draw_spectrum(t, x)
