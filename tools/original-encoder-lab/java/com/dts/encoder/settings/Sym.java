package com.dts.encoder.settings;
public class Sym {
    public static void main(String[] a) throws Exception {
        for (Constants.DestFormat d : Constants.DestFormat.values()) System.out.println("dest " + d + " " + d.getSymbolic());
        for (Constants.StreamType d : Constants.StreamType.values()) System.out.println("stream " + d + " " + d.getSymbolic());
        for (Constants.ChannelLayout d : Constants.ChannelLayout.values()) System.out.println("layout " + d + " " + d.getSymbolic() + " " + d.getMainChannelCount() + " " + d.hasLFE() + " " + d.getDisplay() + " cfgr=" + d.getConfigurator());
    }
}
