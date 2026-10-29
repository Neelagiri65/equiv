def luhn_ok(digits):
    if not digits or any(d < 0 or d > 9 for d in digits):
        return False
    total = 0
    for i, d in enumerate(digits):
        if i % 2 == 1:
            d = d * 2
            if d > 9:
                d -= 9
        total += d
    return total % 10 == 0
