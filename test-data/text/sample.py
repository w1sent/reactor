#!/usr/bin/env python3
"""Highlighting sample."""


def fib(n: int) -> int:
    return n if n < 2 else fib(n - 1) + fib(n - 2)


if __name__ == "__main__":
    print([fib(i) for i in range(10)])
