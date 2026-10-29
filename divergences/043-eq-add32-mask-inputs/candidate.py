def add32(a, b):
    return ((a & 0xFFFFFFFF) + (b & 0xFFFFFFFF)) & 0xFFFFFFFF
