/* 😀 keeps native offsets in bytes. */
#include "extraction.h"
Foo *global;
void run(Foo *ptr, int index) {
    struct Foo tagged;
    Foo aliased, *other;
    Bar bar;
    tagged.id = aliased.id;
    ptr->id = 1;
    ptr->next->id = 2;
    create(index)->id = 3;
    bar.value = 4;
    unknown.id = 0;
}
