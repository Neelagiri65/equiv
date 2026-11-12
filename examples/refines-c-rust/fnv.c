unsigned int fnv1a(const unsigned char *p, unsigned int n) {
    unsigned int h = 2166136261u;
    for (unsigned int i = 0; i < n; i++) {
        h ^= p[i];
        h *= 16777619u;
    }
    return h;
}
