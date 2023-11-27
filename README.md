# Rathernet

This repository contains the code for the Rathernet project. Rathernet is a acoustic userspace network stack for Windows, built from the ground up covering all five layers of the OSI model. It is designed for educational purposes and is not intended to be used in production environments. 

## Installation

To install Rathernet, the following software is required:

1. [Visual Studio 2022](https://visualstudio.microsoft.com/vs/)
2. [LLVM 13.0.0](https://releases.llvm.org/download.html)
3. [AsioAll Driver](http://www.asio4all.org/)
4. [Winpcap](https://www.winpcap.org/install/default.htm)

After installing the required software, clone the repository and change the path in `env.ps1` to the location of your LLVM installation. Then, run the following command in PowerShell:

```powershell
./env.ps1

cargo build --release
```

At this point, you should be able to run the binaries in the `target/release` directory.

## Components

Rathernet is split into five components, described as follows:

1. **Raudio**: this is the lowest layer of the network stack, and is responsible for sending and receiving audio packets. It is implemented using the [AsioAll](http://www.asio4all.org/) driver in an asynchronous manner.

2. **Rather**: this is the second layer of the network stack, and is responsible for sending and receiving Athernet packets. It is not responsible for multiple access control, which is handled by the third layer. Athernet is a custom protocol designed for Rathernet, and is not compatible with Ethernet.

3. **Racsma**: this is the third layer of the network stack, and is responsible for multiple access control. It is implemented using the [CSMA/CD](https://en.wikipedia.org/wiki/Carrier_sense_multiple_access_with_collision_detection) algorithm.

4. **Rateway**: this is the fourth layer of the network stack, and is responsible for routing packets. It includes an adapter to integrate with the Windows operating system and a NAT to interact with the Internet.

5. **Raftp**: this is the fifth layer of the network stack, and is responsible for sending and receiving files. It is implemented using the [File Transfer Protocol](https://en.wikipedia.org/wiki/File_Transfer_Protocol) as an application on top of the network stack.

### Raudio

Raudio includes a library and a client. The library implements two core structs:
* `AudioInputStream` - this struct wraps an audio input stream. It implements the `Stream` trait from the `futures` crate, and can be used to asynchronously read audio samples from the input stream.
* `AudioOutputStream` - this struct wraps an audio output stream. It implements the `Sink` trait from the `futures` crate, and can be used to asynchronously write audio samples to the output stream.

The client is a command line interface that can be used to test the library. It can be used to record audio from a microphone and play it back through the speakers. It can also be used to play audio from a file. Use the `--help` flag to see the available options.

### Rather

Rather includes a library and a client. The library implements two core structs:
* `AtherInputStream` - this struct wraps an audio input stream. It implements the `Stream` trait from the `futures` crate, and can be used to asynchronously read Athernet frames from the input stream.
* `AtherOutputStream` - this struct wraps an audio output stream. It implements the `Sink` trait from the `futures` crate, and can be used to asynchronously write Athernet frames to the output stream.

It also contains utilities for signal processing and error correction, covered in the `signal` and `conv` modules respectively.

The client is a command line interface that can be used to test the library. It can be used to send and receive Athernet frames. Use the `--help` flag to see the available options.

### Racsma

Racsma includes a library and a client. The library implements three core structs:
* `AtewayIoSocket` - this struct creates a reader and a writer on the CSMA/CD daemon, which can be used to asynchronously send and receive frames.
* `AtewaySocketReader` - this struct wraps a reader on the CSMA/CD daemon. It provides a `read` method, which can be used to asynchronously read a packet from the daemon.
* `AtewaySocketWriter` - this struct wraps a writer on the CSMA/CD daemon. It provides a `write` method, which can be used to asynchronously write a packet to the daemon.

The client is a command line interface that can be used to test the library. It can be used to send and receive packets. Use the `--help` flag to see the available options.

### Rateway

Rateway includes a library and a client. The library implements three core structs:
* `AtewayIoAdapter` - this struct creates an tunneled adapter on the operating system, which redirects packets from the operating system to the network stack and vice versa.
* `AtewayIoNAT` - this struct creates a network address translator, which translates packets from the network stack to the Internet and vice versa.
* `AtewayIoSocket` - this struct is a raw socket that can be used to send and receive packets from the Internet, implemented based on the [Winpcap](https://www.winpcap.org/) library as a workaround for the TCP packet limitaion in [Winsock](https://docs.microsoft.com/en-us/windows/win32/winsock/windows-sockets-start-page-2).

The client is a command line interface that can be used to test the library. It can be used to send and receive data from and to the Internet. Use the `--help` flag to see the available options.

### Raftp

Raftp is a FTP client that can be used to send and receive files. It is implemented using the [File Transfer Protocol](https://en.wikipedia.org/wiki/File_Transfer_Protocol) as an application on top of the network stack. It is not a library, and is not intended to be used as such. As an application, it does not depend on the network stack, and can be used with any network stack that implements the five layers of the OSI model.

## Benchmarks

The following benchmarks were performed on a machine with the following specifications:

* CPU: Intel Core i5-10600K @ 4.10GHz
* RAM: 16GB DDR4 @ 3200MHz
* OS: Windows 11 Pro 23H2

The benchmark results are as follows:

| Benchmark | File Size | Bidirectional | Channel Status | Bit Rate | Delay | Time |
| --------- | --------- | ------------- | -------------- | -------- | ----- | ---- |
| Rather    | 1KB       | No            | Wireless       | 1kbps    | 300ms | 12s  |
| Racsma    | 50KB      | Yes           | Wired          | 24kbps   | 150ms | 13s  |
| Racsma    | 50KB      | Yes           | Wired, Noisy   | 24kbps   | 350ms | 33s  |
| Rateway   | 2KB       | No            | Wired          | 24kbps   | 150ms | <1s  |



**Surf the Internet with Rathernet**

![Surf the Internet with Rathernet](assets/aftp/surf.jpg)



## License

This project is licensed under the MPL-2.0 license. See the [LICENSE](LICENSE) file for details.
