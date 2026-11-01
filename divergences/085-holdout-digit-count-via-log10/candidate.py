import math

def digit_count(n):
    if n == 0:
        return 1
    return int(math.log10(abs(n))) + 1
