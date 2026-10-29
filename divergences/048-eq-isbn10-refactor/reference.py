def isbn10_valid(s):
    if len(s) != 10:
        return False
    total = 0
    for i, ch in enumerate(s):
        if ch.isdecimal() and ch.isascii():
            v = int(ch)
        elif ch == "X" and i == 9:
            v = 10
        else:
            return False
        total += (10 - i) * v
    return total % 11 == 0
