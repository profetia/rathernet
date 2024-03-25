import sys

verbose = False


def count_one(byte):
    count = 0
    while byte:
        byte &= byte - 1
        count += 1
    return count


def main(in_file, out_file):
    with open(in_file, "rb") as f:
        ref = f.read()

    with open(out_file, "rb") as f:
        recv = f.read()

    if len(ref) != len(recv):
        print("FAIL: Lengths do not match (0.00%)")
        return

    byte_length = len(ref)
    bit_length = byte_length * 8

    byte_score = 0
    bit_score = 0
    for i in range(byte_length):
        ref_byte = ref[i]
        recv_byte = recv[i]
        if ref_byte == recv_byte:
            byte_score += 1
            bit_score += 8
        elif verbose:
            bit_score += count_one((ref_byte ^ recv_byte))
            print(f"[{i}] ERROR: Expect {ref_byte}, got {recv_byte}")

    if byte_score == byte_length:
        print("PASS: Perfect score (100.00%)")
    else:
        print(
            f"FAIL: {byte_length - byte_score} bytes incorrect ({byte_score / byte_length * 100}%)"
        )
        print(
            f"FAIL: {bit_length - bit_score} bits incorrect ({bit_score / bit_length * 100}%)"
        )


if __name__ == "__main__":
    if sys.argv[1] == "-v":
        verbose = True
        main(sys.argv[2], sys.argv[3])
    else:
        main(sys.argv[1], sys.argv[2])
