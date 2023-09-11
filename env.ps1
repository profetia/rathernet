$env:CPAL_ASIO_DIR = "$(Get-Location)/third_party/asiosdk/"

$env:LIBCLANG_PATH = "your/path/to/LLVM/bin"

vcvarsall.bat amd64
