import math

def digit_count(n):
    if n == 0:
        return 1
    return int(math.log(abs(n), 10)) + 1
