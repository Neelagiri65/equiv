def to_int(s):
    if not s.lstrip("-").isdigit():
        raise TypeError("not an integer")
    return int(s)
