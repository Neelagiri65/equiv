def utf8_size(s):
    return sum(len(c.encode("utf-8")) for c in s)
