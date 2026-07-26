class C {
    void M() {
        var r = new System.IO.StreamReader("");
        try {
            r.Read();
        } finally {
            r.Close();
        }
    }
}
