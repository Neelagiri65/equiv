def unsigned(s):
    return s[1:] if s.startswith(("+", "-")) else s
