def iban_valid(s):
    s = s.replace(" ", "").upper()
    if len(s) < 5 or not s.isascii() or not s.isalnum():
        return False
    rem = 0
    for c in s[4:] + s[:4]:
        if c.isdigit():
            rem = (rem * 10 + int(c)) % 97
        else:
            rem = (rem * 100 + ord(c) - 55) % 97
    return rem == 1
