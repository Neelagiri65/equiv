class PanicException(BaseException):
    """Stand-in for pyo3_runtime.PanicException: same name and base class."""
    __module__ = "pyo3_runtime"

_T = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4]


def week_day(year, month, day):
    if not 1 <= month <= 12:
        raise PanicException("index out of bounds")
    if month < 3:
        year -= 1
    return (year + year // 4 - year // 100 + year // 400 + _T[month - 1] + day) % 7
