using System;
using System.Collections.Generic;

namespace RefactoringGuru.DesignPatterns.Mediator.Conceptual
{
    public interface IMediator
    {
        void Notify(object sender, string message);
        void RegisterComponent(BaseComponent component);
    }

    class ConcreteMediator : IMediator
    {
        private BaseComponent _componentA;
        private BaseComponent _componentB;

        public ConcreteMediator(BaseComponent a, BaseComponent b)
        {
            this._componentA = a;
            this._componentB = b;
        }

        public void Notify(object sender, string message)
        {
            if (message == "A")
            {
                Console.WriteLine("Mediator reacts on A and triggers B.");
                this._componentB.DoB();
            }
            if (message == "B")
            {
                Console.WriteLine("Mediator reacts on B and triggers A.");
                this._componentA.DoA();
            }
        }

        public void RegisterComponent(BaseComponent component)
        {
            Console.WriteLine("Component registered.");
        }
    }

    class BaseComponent
    {
        protected IMediator _mediator;

        public BaseComponent(IMediator mediator = null)
        {
            this._mediator = mediator;
        }

        public void SetMediator(IMediator mediator)
        {
            this._mediator = mediator;
        }
    }

    class ComponentA : BaseComponent
    {
        public void DoA()
        {
            Console.WriteLine("Component A does A.");
            if (this._mediator != null)
            {
                this._mediator.Notify(this, "A");
            }
        }
    }

    class ComponentB : BaseComponent
    {
        public void DoB()
        {
            Console.WriteLine("Component B does B.");
            if (this._mediator != null)
            {
                this._mediator.Notify(this, "B");
            }
        }
    }

    class Program
    {
        static void Main(string[] args)
        {
            ComponentA a = new ComponentA();
            ComponentB b = new ComponentB();
            ConcreteMediator mediator = new ConcreteMediator(a, b);
            a.SetMediator(mediator);
            b.SetMediator(mediator);

            a.DoA();
            b.DoB();
        }
    }
}
