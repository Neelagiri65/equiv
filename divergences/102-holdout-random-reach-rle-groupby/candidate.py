from itertools import groupby


def rle(s):
    return "".join(ch + str(len(list(run))) for ch, run in groupby(s))
