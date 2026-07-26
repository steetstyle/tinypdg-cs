using System;

namespace RefactoringGuru.DesignPatterns.AbstractFactory.Conceptual
{
    public interface IButton
    {
        void Render();
    }

    public interface IDialog
    {
        void Show();
    }

    public interface IGUIFactory
    {
        IButton CreateButton();
        IDialog CreateDialog();
    }

    class WinButton : IButton
    {
        public void Render()
        {
            Console.WriteLine("Windows Button rendered.");
        }
    }

    class WinDialog : IDialog
    {
        public void Show()
        {
            Console.WriteLine("Windows Dialog shown.");
        }
    }

    class WinFactory : IGUIFactory
    {
        public IButton CreateButton()
        {
            return new WinButton();
        }

        public IDialog CreateDialog()
        {
            return new WinDialog();
        }
    }

    class MacButton : IButton
    {
        public void Render()
        {
            Console.WriteLine("Mac Button rendered.");
        }
    }

    class MacDialog : IDialog
    {
        public void Show()
        {
            Console.WriteLine("Mac Dialog shown.");
        }
    }

    class MacFactory : IGUIFactory
    {
        public IButton CreateButton()
        {
            return new MacButton();
        }

        public IDialog CreateDialog()
        {
            return new MacDialog();
        }
    }

    class Program
    {
        static void Main(string[] args)
        {
            IGUIFactory factory = new WinFactory();
            IButton button = factory.CreateButton();
            IDialog dialog = factory.CreateDialog();
            button.Render();
            dialog.Show();
        }
    }
}
