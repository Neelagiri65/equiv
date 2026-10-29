DOUBLED = [0, 2, 4, 6, 8, 1, 3, 5, 7, 9]


def luhn_valid(s):
    if not s or not s.isdecimal():
        return False
    digits = [int(ch) for ch in reversed(s)]
    total = sum(digits[0::2]) + sum(DOUBLED[d] for d in digits[1::2])
    return total % 10 == 0
