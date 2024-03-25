import sys

verbose = False


def main(in_file, out_file):
    with open(in_file, "r") as f:
        ref = f.read()

    with open(out_file, "r") as f:
        recv = f.read()

    if len(ref) != len(recv):
        print("FAIL: Lengths do not match (0.00%)")
        return

    length = len(ref)

    score = 0
    for i in range(length):
        ref_bit = ref[i]
        recv_bit = recv[i]
        if ref_bit == recv_bit:
            score += 1
        elif verbose:
            print(f"[{i}] ERROR: Expect {ref_bit}, got {recv_bit}")

    if score == length:
        print("PASS: Perfect score (100.00%)")
    else:
        print(
            f"FAIL: {length - score} bits incorrect ({score / length * 100}%)"
        )


if __name__ == "__main__":
    if sys.argv[1] == "-v":
        verbose = True
        main(sys.argv[2], sys.argv[3])
    else:
        main(sys.argv[1], sys.argv[2])
