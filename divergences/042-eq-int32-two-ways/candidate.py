def to_int32(n):
    return ((n + 0x80000000) % 0x100000000) - 0x80000000
