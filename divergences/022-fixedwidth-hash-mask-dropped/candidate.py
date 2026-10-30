def hash32(s):
    h = 0
    for c in s:
        h = h * 31 + ord(c)
    return h
