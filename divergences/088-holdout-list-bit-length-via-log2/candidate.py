import math

def bit_lengths(xs):
    return [int(math.log2(x)) + 1 for x in xs]
