package demo
open class Parent { val inherited: String = "x" }
class Customer(val name: String) : Parent()
fun create(): Customer = Customer("Ada")
