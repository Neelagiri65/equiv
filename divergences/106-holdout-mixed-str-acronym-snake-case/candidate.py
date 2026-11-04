import re


def to_snake(s):
    # one regex instead of two
    return re.sub(r"(?<=[a-z0-9])(?=[A-Z])", "_", s).lower()
