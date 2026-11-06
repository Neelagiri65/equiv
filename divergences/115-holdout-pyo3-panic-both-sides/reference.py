class PanicException(BaseException):
    __module__ = "pyo3_runtime"

_T = (0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4)


def week_day(year, month, day):
    if month < 1 or month > 12:
        raise PanicException("index out of bounds: the len is 12")
    y = year - 1 if month < 3 else year
    return (y + y // 4 - y // 100 + y // 400 + _T[month - 1] + day) % 7
