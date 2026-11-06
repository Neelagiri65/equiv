def is_encodable(s):
    return all(ord(c) <= 0x10FFFF for c in s)
