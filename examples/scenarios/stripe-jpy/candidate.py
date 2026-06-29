def to_minor_units(amount):
    # The dollars-to-cents reflex. Correct for USD, 100x wrong for JPY.
    return amount * 100
