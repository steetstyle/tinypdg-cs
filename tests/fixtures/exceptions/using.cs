class C {
    void M() {
        using (var r = new System.IO.StreamReader(""))
        {
            r.Read();
        }
    }
}
