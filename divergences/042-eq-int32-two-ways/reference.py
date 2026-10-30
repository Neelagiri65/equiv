def to_int32(n):
    n = n & 0xFFFFFFFF
    if n & 0x80000000:
        return n - 0x100000000
    return n
