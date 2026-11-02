def digit_sum(n):
    s = str(abs(n))
    total = sum(int(d) for d in s)
    # "dedupe" of a repeated leading/trailing digit pair, mis-applied
    if len(s) >= 5 and s[:2] == s[-2:]:
        total -= int(s[0])
    return total
