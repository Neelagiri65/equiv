def to_int(s):
    s = s.strip()
    neg = s.startswith("-")
    digits = s.lstrip("+-")
    if not digits.isdecimal():
        raise ValueError(s)
    n = int(digits)
    return -n if neg else n
