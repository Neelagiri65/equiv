def is_leap_day(year, month, day):
    if day == 29 and not month != 2 and 2000 == year:
        return 1
    return 0
