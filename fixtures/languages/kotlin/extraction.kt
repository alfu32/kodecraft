package demo.core
import example.Customer as Person
import example.items.*

typealias Maybe = Box<String>?
interface Face {
    fun size(): Int
}
enum class Color { RED, BLUE }
open class Base
class Café<T>(private val name: String, count: Int) : Base(), Face {
    val address: Box<String>? = null
    protected var number = 1
    internal fun work(arg: Person): String {
        val copy = arg
        var value = null
        value = Person()
        val cast = (arg as Person)
        val nested = arg.address.city
        val called = getCustomer().address
        val self = this
        val parent = super
        if (true) { val copy = 2; println(copy) }
        list.map { item -> item.name }
        return "$name"
    }
}

const val LIMIT = 9
fun top(box: Café<String>?): Café<String>? = box
