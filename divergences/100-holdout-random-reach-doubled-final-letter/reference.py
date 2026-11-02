def phonetic_key(s):
    out = []
    for c in s.upper():
        if c.isalpha() and (not out or out[-1] != c):
            out.append(c)
    return "".join(out)
