def luhn_valid(s):
    if not s or not s.isdecimal():
        return False
    total = 0
    for i, ch in enumerate(reversed(s)):
        d = int(ch)
        if i % 2 == 1:
            d = d * 2 % 9
        total += d
    return total % 10 == 0
