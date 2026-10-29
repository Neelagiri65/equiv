def ean_check_digit(digits):
    total = 0
    for i, d in enumerate(reversed(digits)):
        total += d * (3 if i % 2 == 0 else 1)
    return 10 - total % 10
