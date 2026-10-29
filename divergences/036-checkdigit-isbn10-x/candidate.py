def isbn10_valid(s):
    if len(s) != 10 or not (s.isdecimal() and s.isascii()):
        return False
    return sum((10 - i) * int(ch) for i, ch in enumerate(s)) % 11 == 0
