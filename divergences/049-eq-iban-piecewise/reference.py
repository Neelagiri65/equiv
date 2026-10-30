def iban_valid(s):
    s = s.replace(" ", "").upper()
    if len(s) < 5 or not s.isascii() or not s.isalnum():
        return False
    r = s[4:] + s[:4]
    digits = "".join(str(int(c)) if c.isdigit() else str(ord(c) - 55) for c in r)
    return int(digits) % 97 == 1
