def clamp_pct(n):
    if n < 0:
        return 0
    if n > 100:
        return 100
    return n
