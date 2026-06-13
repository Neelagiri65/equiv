"""Example module checked by the equiv review action.

Each function listed in the root .equiv-review manifest is checked on every PR:
its new version is tested against its base-branch version. A behaviour-preserving
change passes; a behaviour-changing one fails with the input that distinguishes
them. See the docs and the equiv review action.
"""


def total(n):
    """Sum of 1..n (0 for n <= 0)."""
    s = 0
    for i in range(1, max(n, 0) + 1):
        s += i
    return s
