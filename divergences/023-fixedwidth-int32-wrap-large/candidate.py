def to_int32(n):
    if -0x80000000 <= n <= 0xFFFFFFFF:
        return n
    return ((n + 0x80000000) % 0x100000000) - 0x80000000
