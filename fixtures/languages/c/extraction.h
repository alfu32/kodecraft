typedef struct Foo {
    int id;
    const char *name;
    struct Foo *next;
} Foo;
typedef struct { int value; } Bar;
typedef union Choice { int number; char letter; } Choice;
enum Color { RED, BLUE = 2 };
Foo *create(int count);
