def isbn10_valid(s):
    if len(s) != 10:
        return False
    body, last = s[:9], s[9]
    if not (body.isdecimal() and body.isascii()):
        return False
    if last == "X":
        check = 10
    elif last.isdecimal() and last.isascii():
        check = int(last)
    else:
        return False
    return (sum((10 - i) * int(c) for i, c in enumerate(body)) + check) % 11 == 0
